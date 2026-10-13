//! Shared by the integration tests that run on the `tests/fixtures/zbrain-like` fixture.
//!
//! The fixture is copied into a fresh temp dir per test so the resolve is hermetic: the real
//! ancestors of the harn checkout (and its git root) never enter the instruction chain. At copy
//! time `@HOME@` / `@PROJECT@` in every `.json` become the canonical temp paths, the
//! `PROJECT_SLUG` memory dir is renamed to the slug computed from the project path, and a `.git`
//! dir makes `project/` its own repo root.

#![allow(dead_code)]

use harn::model::Target;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Fixture {
    pub root: PathBuf,
    pub home: PathBuf,
    pub project: PathBuf,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

pub fn slug(path: &Path) -> String {
    path.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().into_owned();
        let src = entry.path();
        if src.is_dir() {
            copy_dir(&src, &to.join(&name));
        } else {
            std::fs::copy(&src, to.join(&name)).unwrap();
        }
    }
}

fn substitute(dir: &Path, home: &Path, project: &Path) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            substitute(&path, home, project);
        } else if path.extension().is_some_and(|e| e == "json") {
            let text = std::fs::read_to_string(&path).unwrap();
            let text = text
                .replace("@HOME@", &home.display().to_string())
                .replace("@PROJECT@", &project.display().to_string());
            std::fs::write(&path, text).unwrap();
        }
    }
}

pub fn fixture() -> Fixture {
    static N: AtomicUsize = AtomicUsize::new(0);
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/zbrain-like");
    let root = std::env::temp_dir().join(format!(
        "harn-resolve-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&root);
    copy_dir(&src, &root);
    let root = std::fs::canonicalize(&root).unwrap();
    let home = root.join("home");
    let project = root.join("project");
    std::fs::create_dir(project.join(".git")).unwrap();
    std::fs::rename(
        home.join(".claude/projects/PROJECT_SLUG"),
        home.join(".claude/projects").join(slug(&project)),
    )
    .unwrap();
    substitute(&root, &home, &project);
    Fixture {
        root,
        home,
        project,
    }
}

pub fn target(f: &Fixture) -> Target {
    Target {
        dir: f.project.clone(),
        home: f.home.clone(),
        env: BTreeMap::new(),
        model: None,
        context_window: None,
        bytes_per_token: None,
    }
}
