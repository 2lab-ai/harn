//! Capture readers (WU-D): an OTEL raw-body directory or an llmux `raw-io.jsonl` record. A record
//! is used only when its `RunContext` matches the target; "the newest" is never a selection rule.
//!
//! The working directory of a record comes from the request itself, strongest evidence first:
//! the `# Environment` section's `Primary working directory`, then the session's `init` event,
//! then (weak, equality only) the directory of the deepest project-layer `CLAUDE.md`. A record
//! must also be a session's first request (no assistant message yet) — later requests carry the
//! conversation, which is not harness.

use super::claude::{context, find_init_event, parse_body, version_triple, InitEvent, ParsedBody};
use super::Capture;
use crate::model::Target;
use anyhow::{anyhow, bail, Context};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// How far back from its end an llmux `raw-io.jsonl` is scanned for a matching record.
pub const LLMUX_SCAN_LIMIT: u64 = 4 << 30;

/// A line longer than this is skipped by the llmux scan (a record holds two bodies of at most
/// llmux's `max_body_bytes`, 8 MiB by default, plus escapes).
const LLMUX_LINE_LIMIT: usize = 256 << 20;

/// `harn verify --capture <path>`: dispatches on what `path` is (OTEL raw-body dir, its
/// `index.jsonl`, one `*.request.json`, or an llmux `raw-io.jsonl` file) and returns the one record
/// whose `RunContext` matches `target`, or Err naming the mismatch.
pub fn read_capture(path: &Path, target: &Target) -> anyhow::Result<Capture> {
    if path.is_dir() {
        for dir in [path.to_path_buf(), path.join("otel-raw")] {
            if dir.join("index.jsonl").is_file() {
                return read_otel_dir(&dir, target);
            }
        }
        bail!(
            "{}: not an OTEL raw-body directory (no index.jsonl); Codex trace bundles are not read before P0.5",
            path.display()
        );
    }
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "index.jsonl" {
        let dir = path.parent().unwrap_or(Path::new("."));
        return read_otel_dir(dir, target);
    }
    if name == "stream.jsonl" {
        bail!(
            "{}: a stream-json transcript has no request body; pass the OTEL raw-body directory",
            path.display()
        );
    }
    if name.ends_with(".request.json") {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let parsed = parse_body(&bytes, None).with_context(|| path.display().to_string())?;
        return bind(
            parsed,
            None,
            target,
            "otel-file",
            modified(path),
            bytes.len(),
        );
    }
    read_llmux_file(path, target)
}

/// The newest request listed in `<dir>/index.jsonl` that is a first request bound to
/// `target.dir`; the session's `init` event is read from `stream.jsonl` in `dir` or its parent
/// when its `session_id` matches.
pub fn read_otel_dir(dir: &Path, target: &Target) -> anyhow::Result<Capture> {
    let index_path = dir.join("index.jsonl");
    let index = std::fs::read_to_string(&index_path)
        .with_context(|| format!("reading {}", index_path.display()))?;
    let entries: Vec<Value> = index
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    if entries.is_empty() {
        bail!("{} lists no request", index_path.display());
    }
    let stream_init = [Some(dir.to_path_buf()), dir.parent().map(Path::to_path_buf)]
        .into_iter()
        .flatten()
        .map(|d| d.join("stream.jsonl"))
        .find(|p| p.is_file())
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| find_init_event(&s).ok());

    let mut reasons = Vec::new();
    for entry in entries.iter().rev() {
        let Some(file) = entry["request_file"].as_str() else {
            continue;
        };
        let session = entry["session_id"].as_str();
        let init = stream_init
            .clone()
            .filter(|i| i.session_id.is_none() || i.session_id.as_deref() == session);
        let bytes = match std::fs::read(dir.join(file)) {
            Ok(bytes) => bytes,
            Err(err) => {
                reasons.push(format!("{file}: {err}"));
                continue;
            }
        };
        let captured_at = entry["timestamp"].as_str().unwrap_or_default().to_string();
        let bound = parse_body(&bytes, init.as_ref())
            .and_then(|parsed| bind(parsed, init, target, "otel-dir", captured_at, bytes.len()));
        match bound {
            Ok(capture) => return Ok(capture),
            Err(err) => reasons.push(format!("{file}: {err:#}")),
        }
    }
    bail!(
        "no request in {} is a first request bound to {}:\n  {}",
        dir.display(),
        target.dir.display(),
        reasons.join("\n  ")
    )
}

/// One llmux `raw-io.jsonl` line (`llmux/src/proxy/raw_io.rs` `RawIoRecord`: `ts_ms`, `model`,
/// `request_body`, ...). The `RunContext` is derived from the body (cwd, `cc_version` billing
/// header, `model`); Err if underivable, truncated, not a first request, or bound elsewhere.
pub fn read_llmux_record(line: &str, target: &Target) -> anyhow::Result<Capture> {
    let record: Value =
        serde_json::from_str(line.trim()).context("llmux raw-io record: not JSON")?;
    let body = record["request_body"]
        .as_str()
        .ok_or_else(|| anyhow!("llmux raw-io record has no request_body"))?;
    let captured_at = record["ts_ms"]
        .as_u64()
        .and_then(|ms| {
            time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000).ok()
        })
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default();
    let parsed = parse_body(body.as_bytes(), None)?;
    let mut capture = bind(
        parsed,
        None,
        target,
        "llmux-raw-io",
        captured_at,
        body.len(),
    )?;
    if capture.context.model.is_none() {
        capture.context.model = record["model"].as_str().map(str::to_string);
    }
    Ok(capture)
}

/// Binds a parsed body to `target` or says why not.
fn bind(
    parsed: ParsedBody,
    init: Option<InitEvent>,
    target: &Target,
    kind: &str,
    captured_at: String,
    body_len: usize,
) -> anyhow::Result<Capture> {
    let (cwd, evidence) = derived_cwd(&parsed, init.as_ref()).ok_or_else(|| {
        anyhow!("cannot derive the working directory (no `# Environment` section, no init event, no project CLAUDE.md)")
    })?;
    if cwd != target.dir {
        bail!(
            "bound to {} ({evidence}), not to the target {}",
            cwd.display(),
            target.dir.display()
        );
    }
    if !parsed.facts.first_request {
        bail!(
            "not a session's first request ({} messages, the conversation has started)",
            parsed.facts.messages
        );
    }
    let version = init
        .as_ref()
        .and_then(|i| i.claude_code_version.clone())
        .or_else(|| parsed.facts.cc_version.clone())
        .ok_or_else(|| {
            anyhow!("cannot derive the Claude Code version (no init event, no `cc_version` billing header)")
        })?;
    let model = parsed
        .facts
        .model
        .clone()
        .or_else(|| init.as_ref().and_then(|i| i.model.clone()));
    if let (Some(want), Some(got)) = (&target.model, &model) {
        if !same_model(want, got) {
            bail!("model {got}, not the target's {want}");
        }
    }
    let mut tree = parsed.tree;
    tree.target_dir = cwd.clone();
    tree.agent_version = Some(version_triple(&version));
    Ok(Capture {
        context: context(cwd, Some(version), model, kind, captured_at),
        body_bytes: body_len as u64,
        regions: parsed.regions,
        tree,
        init,
    })
}

/// The record's working directory (canonical when it exists here) and which evidence gave it.
fn derived_cwd(parsed: &ParsedBody, init: Option<&InitEvent>) -> Option<(PathBuf, &'static str)> {
    let canonical = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    if let Some(cwd) = &parsed.facts.cwd {
        return Some((canonical(cwd), "Environment: Primary working directory"));
    }
    if let Some(cwd) = init.and_then(|i| i.cwd.as_ref()) {
        return Some((canonical(cwd), "init event cwd"));
    }
    parsed
        .facts
        .project_dirs
        .iter()
        .max_by_key(|d| d.components().count())
        .map(|d| (canonical(d), "deepest project CLAUDE.md directory"))
}

/// `opus` matches `claude-opus-5-5`; a `[1m]` context suffix is ignored.
fn same_model(want: &str, got: &str) -> bool {
    let base = |m: &str| m.split('[').next().unwrap_or(m).trim().to_ascii_lowercase();
    let (want, got) = (base(want), base(got));
    want == got || got.contains(&want) || want.contains(&got)
}

fn modified(path: &Path) -> String {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(time::OffsetDateTime::from)
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_default()
}

/// Scans an llmux `raw-io.jsonl` newest-first for the first record bound to `target.dir`
/// (within [`LLMUX_SCAN_LIMIT`] of the end). Lines that do not mention the target directory are
/// skipped without parsing.
fn read_llmux_file(path: &Path, target: &Target) -> anyhow::Result<Capture> {
    let needle = target.dir.to_string_lossy().into_owned().into_bytes();
    let mut file =
        std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let len = file.metadata()?.len();
    let floor = len.saturating_sub(LLMUX_SCAN_LIMIT);
    let mut lines = ReverseLines::new(&mut file, len, floor);
    let (mut mentions, mut reasons) = (0usize, Vec::new());
    while let Some(line) = lines.next_line()? {
        if !contains(&line, &needle) || !contains(&line, b"\"request_body\"") {
            continue;
        }
        mentions += 1;
        match std::str::from_utf8(&line)
            .map_err(anyhow::Error::from)
            .and_then(|l| read_llmux_record(l, target))
        {
            Ok(capture) => return Ok(capture),
            Err(err) if reasons.len() < 3 => reasons.push(format!("{err:#}")),
            Err(_) => {}
        }
    }
    let scanned = len - floor;
    let mut msg = format!(
        "no llmux record in the last {} of {} is a first request bound to {} ({mentions} records mention it)",
        human_bytes(scanned),
        path.display(),
        target.dir.display()
    );
    for reason in reasons {
        msg.push_str("\n  ");
        msg.push_str(&reason);
    }
    Err(anyhow!(msg))
}

fn human_bytes(n: u64) -> String {
    match n {
        n if n >= 1 << 30 => format!("{:.1} GiB", n as f64 / (1u64 << 30) as f64),
        n if n >= 1 << 20 => format!("{:.1} MiB", n as f64 / (1u64 << 20) as f64),
        n => format!("{n} B"),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    needle.is_empty() || haystack.windows(needle.len()).any(|w| w == needle)
}

/// Lines of a file from its end towards `floor`, newest first, without the newline. The partial
/// line at the read front is kept as a list of chunks so a long line is copied once.
struct ReverseLines<'a, F: Read + Seek> {
    file: &'a mut F,
    pos: u64,
    floor: u64,
    /// Complete lines found in the last chunk, newest first.
    ready: VecDeque<Vec<u8>>,
    /// The line that starts before `pos`, in file order.
    partial: VecDeque<Vec<u8>>,
    partial_len: usize,
    /// The line at the read front is over [`LLMUX_LINE_LIMIT`] and is being skipped.
    dropping: bool,
}

impl<'a, F: Read + Seek> ReverseLines<'a, F> {
    const CHUNK: u64 = 1 << 20;

    fn new(file: &'a mut F, len: u64, floor: u64) -> Self {
        ReverseLines {
            file,
            pos: len,
            floor,
            ready: VecDeque::new(),
            partial: VecDeque::new(),
            partial_len: 0,
            dropping: false,
        }
    }

    fn take_partial(&mut self) -> Vec<u8> {
        let mut line = Vec::with_capacity(self.partial_len);
        for chunk in self.partial.drain(..) {
            line.extend_from_slice(&chunk);
        }
        self.partial_len = 0;
        line
    }

    fn next_line(&mut self) -> anyhow::Result<Option<Vec<u8>>> {
        loop {
            if let Some(line) = self.ready.pop_front() {
                return Ok(Some(line));
            }
            if self.pos <= self.floor {
                // The line at the floor is complete only when the floor is the file start.
                let dropped = std::mem::take(&mut self.dropping);
                let line = self.take_partial();
                return Ok((self.floor == 0 && !dropped && !line.is_empty()).then_some(line));
            }
            let start = self.pos.saturating_sub(Self::CHUNK).max(self.floor);
            let mut chunk = vec![0u8; (self.pos - start) as usize];
            self.file.seek(SeekFrom::Start(start))?;
            self.file.read_exact(&mut chunk)?;
            self.pos = start;
            let mut end = chunk.len();
            while let Some(nl) = chunk[..end].iter().rposition(|&b| b == b'\n') {
                if std::mem::take(&mut self.dropping) {
                    self.take_partial();
                } else {
                    let mut line = chunk[nl + 1..end].to_vec();
                    if !self.partial.is_empty() {
                        let tail = self.take_partial();
                        line.extend_from_slice(&tail);
                    }
                    if !line.is_empty() {
                        self.ready.push_back(line);
                    }
                }
                end = nl;
            }
            chunk.truncate(end);
            if self.dropping {
                continue;
            }
            if self.partial_len + chunk.len() > LLMUX_LINE_LIMIT {
                // Too long to be a record we can use: skip it, keep scanning past it.
                self.take_partial();
                self.dropping = true;
            } else if !chunk.is_empty() {
                self.partial_len += chunk.len();
                self.partial.push_front(chunk);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn all_lines(data: &[u8], floor: u64) -> Vec<String> {
        let mut cursor = Cursor::new(data.to_vec());
        let len = data.len() as u64;
        let mut lines = ReverseLines::new(&mut cursor, len, floor);
        let mut out = Vec::new();
        while let Some(line) = lines.next_line().unwrap() {
            out.push(String::from_utf8(line).unwrap());
        }
        out
    }

    #[test]
    fn reverse_lines_are_newest_first_and_join_chunks() {
        let long = "x".repeat(3 * (1 << 20) + 17);
        let data = format!("first\n{long}\nthird\n");
        let lines = all_lines(data.as_bytes(), 0);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "third");
        assert_eq!(lines[1].len(), long.len());
        assert_eq!(lines[2], "first");
        assert_eq!(all_lines(b"a\nb", 0), vec!["b", "a"]);
    }

    #[test]
    fn reverse_lines_stop_at_the_floor_without_a_cut_line() {
        // floor inside "first": that line is incomplete and must not be yielded.
        assert_eq!(all_lines(b"first\nsecond\n", 2), vec!["second"]);
    }

    #[test]
    fn model_match_ignores_aliases_and_context_suffix() {
        assert!(same_model("opus", "claude-opus-5-5"));
        assert!(same_model("claude-opus-5-5[1m]", "claude-opus-5-5"));
        assert!(!same_model("sonnet", "claude-opus-5-5"));
    }
}
