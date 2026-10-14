//! Exec-policy rules files (`research/02` §5), as codex-cli 0.160.1 loads them
//! (`core/src/exec_policy.rs:662-708`, `exec_policy.rs:1121-1165`): for every enabled config layer
//! with a folder, lowest first — system `/etc/codex`, user `$CODEX_HOME` (also the profile's
//! folder; listed once), each explicitly trusted project `.codex` from the project root down —
//! every regular file directly in `<folder>/rules/` whose extension is `rules`, sorted by path. A
//! symlink is not a regular file there (`DirEntry::file_type` does not follow it), so it is not
//! loaded. Untrusted project layers are not listed (the tree's `trust:` signal says so); rules in
//! `requirements.toml` are enforced on top and not modelled (a warning names them).
//!
//! A rules file is a command policy, not prompt text; it reaches the model only as its `allow`
//! prefixes, rendered as `## Approved command prefixes` inside the permissions block
//! (`prompts/src/permissions_instructions.rs:33-34`, `permissions_instructions.rs:286-345`) — for
//! approval policy `on-request` (`on-failure` is a serde alias of it,
//! `protocol/src/protocol.rs:994-996`) and `granular` only. The policy defaults to `on-request`, or to
//! `untrusted` (no prefixes) when the active project is explicitly untrusted
//! (`core/src/config/mod.rs:3726-3744`). With `include_permissions_instructions = false` the
//! compact block that replaces the permissions block (`core/src/session/world_state.rs:168-201`)
//! adds nothing to the first request: the initial context renders every section against an
//! absent previous state (`core/src/session/mod.rs:4451`,
//! `core/src/context/world_state/mod.rs:398-399`, `440-443`), and the compact block renders
//! only prefixes added since a known state (`core/src/context/world_state/compact_permissions.rs:39-50`)
//! — later, as "approved command prefix saved" messages. So a file row is mode `always` with cost
//! `?` (its share of the permissions block is unknown until a capture) when prefixes render, else
//! `runtime`. No config key turns one file off: `Knob::None`.

use super::config::Source;
use super::{new_item, Ctx};
use crate::model::{Cost, Item, Kind, Knob, Layer, Mode};
use crate::resolve::claude::cost_na;
use serde_json::Value;
use std::path::{Path, PathBuf};

const EXTENSION: &str = "rules";

pub(crate) fn collect(ctx: &Ctx) -> Vec<Item> {
    let mut folders: Vec<(PathBuf, Layer)> = vec![
        (ctx.system_dir.clone(), Layer::Managed),
        (ctx.codex_home.clone(), Layer::User),
    ];
    for layer in &ctx.config.layers {
        if layer.source == Source::Project && layer.enabled() {
            if let Some(folder) = layer.path.as_ref().and_then(|p| p.parent()) {
                folders.push((folder.to_path_buf(), Layer::Project));
            }
        }
    }
    let (renders, why_mode) = prefixes_render(ctx);
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (folder, layer) in folders {
        if seen.contains(&folder) {
            continue;
        }
        seen.push(folder.clone());
        for path in rule_files(ctx, &folder.join("rules")) {
            let real = ctx.fs.realpath(&path);
            let mut item = new_item(
                Kind::PermissionRule,
                layer,
                &real.display().to_string(),
                &ctx.display(&path),
            );
            item.origin.path = Some(real);
            item.origin.why = vec![
                "exec-policy rules file (Starlark prefix_rule; core/src/exec_policy.rs:662-708)"
                    .to_string(),
                why_mode.clone(),
            ];
            if renders {
                item.mode = Mode::Always;
                item.cost = Cost::default();
                item.origin.why.push(
                    "only its `allow` prefixes render (≤ 100 prefixes / 5,000 B for all files); size unknown until a capture"
                        .to_string(),
                );
            } else {
                item.mode = Mode::Runtime;
                item.cost = cost_na(ctx.fs.len(&path));
            }
            item.knobs = vec![Knob::None {
                reason: "no config key turns one rules file off; remove it or rename it to a name not ending in .rules"
                    .to_string(),
            }];
            out.push(item);
        }
    }
    out
}

/// `*.rules` regular files directly in `dir`, sorted.
fn rule_files(ctx: &Ctx, dir: &Path) -> Vec<PathBuf> {
    ctx.fs
        .read_dir(dir)
        .into_iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(EXTENSION))
        .filter(|p| {
            std::fs::symlink_metadata(p)
                .map(|m| m.file_type().is_file())
                .unwrap_or(false)
        })
        .collect()
}

/// Whether approved prefixes reach the request, and why.
fn prefixes_render(ctx: &Ctx) -> (bool, String) {
    if let Some((false, layer)) = ctx.config.get_bool("/include_permissions_instructions") {
        return (
            false,
            format!(
                "{} = false: the compact block that replaces the permissions block (core/src/session/world_state.rs:168-201) renders nothing in the first request — it is rendered in full against an absent previous state (core/src/session/mod.rs:4451, core/src/context/world_state/mod.rs:398-399, 440-443) and returns None then (core/src/context/world_state/compact_permissions.rs:39-50); prefixes reach the model only later, as \"approved command prefix saved\" messages",
                layer.at("/include_permissions_instructions")
            ),
        );
    }
    match ctx.config.get("/approval_policy") {
        Some((Value::String(policy), layer)) => {
            let at = layer.at("/approval_policy");
            match policy.as_str() {
                "on-request" => (
                    true,
                    format!("{at} = \"on-request\": approved prefixes render in the permissions block"),
                ),
                "on-failure" => (
                    true,
                    format!("{at} = \"on-failure\": an alias of on-request (protocol/src/protocol.rs:994-996); approved prefixes render in the permissions block"),
                ),
                "never" => (
                    false,
                    format!("{at} = \"never\": no approved-prefix section; rules apply at run time only"),
                ),
                "untrusted" => (
                    false,
                    format!("{at} = \"untrusted\": codex refuses to start with it (core/src/config/mod.rs:3726-3731)"),
                ),
                other => (
                    false,
                    format!("{at} = \"{other}\": not a codex approval policy"),
                ),
            }
        }
        Some((Value::Object(_), layer)) => (
            true,
            format!(
                "{} = granular: approved prefixes render in the permissions block",
                layer.at("/approval_policy")
            ),
        ),
        Some((_, layer)) => (
            false,
            format!(
                "{}: not a codex approval policy",
                layer.at("/approval_policy")
            ),
        ),
        None if ctx.config.active_project.is_untrusted() => (
            false,
            "approval_policy unset and the project is untrusted: policy `untrusted` has no approved-prefix section (core/src/config/mod.rs:3734-3744)"
                .to_string(),
        ),
        None => (
            true,
            "approval_policy unset: on-request, approved prefixes render in the permissions block (core/src/config/mod.rs:3734-3744)"
                .to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{walk, Target, Tree};
    use crate::resolve::codex::resolve_with;
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
            "harn-codex-rules-{}-{}",
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

    fn resolve(root: &Path) -> Tree {
        let target = Target {
            dir: root.join("repo"),
            home: root.join("home"),
            env: BTreeMap::new(),
            model: None,
            context_window: None,
            bytes_per_token: None,
        };
        resolve_with(&target, None, &root.join("etc/codex")).unwrap()
    }

    fn rules(tree: &Tree) -> Vec<&Item> {
        walk(tree)
            .filter(|i| i.kind == Kind::PermissionRule)
            .collect()
    }

    const RULE: &str = "prefix_rule(pattern = [\"git\", \"status\"], decision = \"allow\")\n";

    #[test]
    fn rules_files_come_from_system_user_and_trusted_projects_only() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        let codex = root.join("home/.codex");
        write(&codex.join("rules/default.rules"), RULE);
        write(&codex.join("rules/b.rules"), RULE);
        write(&codex.join("rules/notes.txt"), "not a rules file");
        write(&codex.join("rules/nested/x.rules"), RULE);
        #[cfg(unix)]
        std::os::unix::fs::symlink(codex.join("rules/b.rules"), codex.join("rules/link.rules"))
            .unwrap();
        write(&root.join("etc/codex/rules/corp.rules"), RULE);
        write(&root.join("repo/.codex/rules/p.rules"), RULE);
        write(&codex.join("config.toml"), "");

        let tree = resolve(root);
        let rows: Vec<(String, Layer)> = rules(&tree)
            .iter()
            .map(|i| (i.label.clone(), i.origin.layer))
            .collect();
        assert_eq!(
            rows,
            [
                (
                    root.join("etc/codex/rules/corp.rules")
                        .display()
                        .to_string(),
                    Layer::Managed
                ),
                ("~/.codex/rules/b.rules".to_string(), Layer::User),
                ("~/.codex/rules/default.rules".to_string(), Layer::User),
            ],
            "project rules wait for trust; symlinks, subdirs and other extensions never load"
        );

        write(
            &codex.join("config.toml"),
            &format!(
                "[projects.\"{}\"]\ntrust_level = \"trusted\"\n",
                root.join("repo").display()
            ),
        );
        let tree = resolve(root);
        let project = rules(&tree)
            .into_iter()
            .find(|i| i.label == ".codex/rules/p.rules")
            .expect("trusted project rules load");
        assert_eq!(project.origin.layer, Layer::Project);
        assert!(matches!(project.knobs.as_slice(), [Knob::None { .. }]));
    }

    #[test]
    fn approval_policy_decides_whether_prefixes_reach_the_request() {
        let t = tmp();
        let root = &t.0;
        write(&root.join("repo/.git/HEAD"), "ref: refs/heads/main\n");
        let codex = root.join("home/.codex");
        write(&codex.join("rules/default.rules"), RULE);
        let mode = |config: &str| {
            write(&codex.join("config.toml"), config);
            let tree = resolve(root);
            let row = rules(&tree)[0].clone();
            (row.mode, row.cost.not_applicable)
        };
        // Default on-request: rendered in the permissions block, size unknown.
        assert_eq!(mode(""), (Mode::Always, false));
        assert_eq!(mode("approval_policy = \"never\"\n"), (Mode::Runtime, true));
        // `on-failure` is a serde alias of `on-request` (`protocol/src/protocol.rs:994-996`).
        assert_eq!(
            mode("approval_policy = \"on-failure\"\n"),
            (Mode::Always, false)
        );
        assert_eq!(
            mode("approval_policy = { granular = { rules = true } }\n"),
            (Mode::Always, false)
        );
        // With the permissions block off, the compact block that replaces it renders nothing in
        // the first request (it only reports prefixes saved later), whatever the policy.
        assert_eq!(
            mode("include_permissions_instructions = false\n"),
            (Mode::Runtime, true)
        );
        assert_eq!(
            mode("approval_policy = \"on-request\"\ninclude_permissions_instructions = false\n"),
            (Mode::Runtime, true)
        );
        // An explicitly untrusted project defaults to `untrusted`: no prefix section.
        let untrusted = format!(
            "[projects.\"{}\"]\ntrust_level = \"untrusted\"\n",
            root.join("repo").display()
        );
        assert_eq!(mode(&untrusted), (Mode::Runtime, true));
    }
}
