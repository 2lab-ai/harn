//! Auto-memory (`research/01` §9): `~/.claude/projects/<slug>/memory/MEMORY.md` (or
//! `autoMemoryDirectory`), injected up to the first 200 lines or 25 KB, whichever comes first.
//! `<slug>` = the main repository root (worktrees share it; the target dir outside git) with every
//! non-alphanumeric character turned into `-` (the 2026-10-13 `init` event: `~/2lab.ai/zbrain` →
//! `-Users-zhugehyuk-2lab-ai-zbrain`). Both caps are byte-denominated and applied here; the byte
//! unit (25,000 vs 25,600) and the cut form are unverified, so a binding byte cap marks the
//! prediction `~`. `cost.predicted` = the embedded section (`Contents of <path> (user's
//! auto-memory, persists across conversations):` + 3 + the capped text). Off: `autoMemoryEnabled: false` (any layer), `CLAUDE_CODE_DISABLE_AUTO_MEMORY=1`
//! (`0` forces it on), `CLAUDE_CODE_DISABLE_CLAUDE_MDS=1` → the item is absent.

use super::{env_knob, json_string, main_repo_root, new_item, setting_knob, Ctx};
use crate::model::{Cost, Item, Kind, Layer, Mode, Scope};
use serde_json::json;
use std::path::{Path, PathBuf};

const MAX_LINES: usize = 200;
const MAX_BYTES: u64 = 25_000;

pub(crate) fn slug(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Bytes of the first `MAX_LINES` lines (newlines included).
fn line_capped(bytes: &[u8]) -> u64 {
    let mut lines = 0;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            lines += 1;
            if lines == MAX_LINES {
                return (i + 1) as u64;
            }
        }
    }
    bytes.len() as u64
}

pub(crate) fn collect(ctx: &mut Ctx) -> Option<Item> {
    let forced_on = ctx
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
        .is_some_and(|v| v.trim() == "0");
    if !forced_on
        && (ctx.env_on("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
            || ctx.env_on("CLAUDE_CODE_DISABLE_CLAUDE_MDS")
            || ctx
                .settings
                .get_bool("/autoMemoryEnabled")
                .is_some_and(|(b, _)| !b))
    {
        return None;
    }
    let root = main_repo_root(ctx).unwrap_or_else(|| ctx.dir().to_path_buf());
    let (dir, dir_why): (PathBuf, String) = match ctx.settings.get_str("/autoMemoryDirectory") {
        Some((d, f)) => {
            let d = match d.strip_prefix("~/") {
                Some(rest) => ctx.home().join(rest),
                None => PathBuf::from(d),
            };
            // The value as a JSON string: a path may hold ` = ` or end in `(…)`.
            let why = format!(
                "{} = {}",
                f.at("/autoMemoryDirectory"),
                json_string(&d.display().to_string())
            );
            (d, why)
        }
        None => (
            ctx.home()
                .join(".claude/projects")
                .join(slug(&root))
                .join("memory"),
            format!("memory dir slug of {}", root.display()),
        ),
    };
    let file = dir.join("MEMORY.md");
    let bytes = ctx.fs.read(&file)?;
    let source = bytes.len() as u64;
    let by_lines = line_capped(&bytes);
    let capped = by_lines.min(MAX_BYTES);
    let predicted = super::instructions::section_bytes(
        &super::fs::normalize_lexical(&file),
        "user's auto-memory, persists across conversations",
        capped,
    );
    let real = ctx.fs.realpath(&file);
    let mut item = new_item(
        Kind::Memory,
        Layer::User,
        &real.display().to_string(),
        &format!("{} (auto-memory)", ctx.display(&real)),
    );
    item.mode = Mode::Always;
    item.cost = Cost {
        source: Some(source),
        predicted: Some(predicted),
        predicted_is_estimate: by_lines > MAX_BYTES,
        ..Cost::default()
    };
    item.origin.path = Some(real);
    item.origin.why = vec![dir_why];
    if capped < source {
        let lines = bytes.iter().filter(|b| **b == b'\n').count();
        item.origin.why.push(format!(
            "capped: {source} B / {lines} lines on disk → first {MAX_LINES} lines or 25 KB = {capped} B"
        ));
    }
    item.knobs = vec![
        setting_knob(
            ctx,
            Scope::Local,
            "/autoMemoryEnabled",
            json!(false),
            Some("this project only; the /memory toggle writes user settings".to_string()),
        ),
        setting_knob(ctx, Scope::User, "/autoMemoryEnabled", json!(false), None),
        env_knob(
            "CLAUDE_CODE_DISABLE_AUTO_MEMORY",
            "1",
            Scope::Local,
            Some("[unverified] whether project/local settings env may set it"),
        ),
    ];
    Some(item)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_replaces_every_non_alphanumeric() {
        assert_eq!(
            slug(Path::new("/Users/zhugehyuk/2lab.ai/zbrain")),
            "-Users-zhugehyuk-2lab-ai-zbrain"
        );
    }

    #[test]
    fn line_cap_counts_newlines() {
        let text: String = (0..250).map(|i| format!("{i}\n")).collect();
        let expected: usize = (0..200).map(|i: i32| format!("{i}\n").len()).sum();
        assert_eq!(line_capped(text.as_bytes()), expected as u64);
        assert_eq!(line_capped(b"a\nb"), 3);
    }
}
