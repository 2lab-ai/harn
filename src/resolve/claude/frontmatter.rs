//! The YAML subset that `SKILL.md`, agent, command and rule frontmatter use in practice: top-level
//! `key: value` with plain, `"double"`, `'single'`, `|`/`>` block (with `-`/`+` chomping) and
//! `[flow]` scalars, and `- item` lists. Nested maps are skipped (read as `null`). Styles measured
//! on the author's machine 2026-10-13: plain 317, double-quoted 55, `>`/`>-` 23, single-quoted 4.

use serde_json::{Map, Value};

/// `(yaml, body)` of a `---`-fenced file; None when the file has no frontmatter.
pub(crate) fn split(text: &str) -> Option<(&str, &str)> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let rest = text.strip_prefix("---")?;
    let rest = rest
        .strip_prefix("\r\n")
        .or_else(|| rest.strip_prefix('\n'))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        let trimmed = line.trim_end_matches(['\n', '\r']);
        if trimmed == "---" || trimmed == "..." {
            return Some((&rest[..offset], &rest[offset + line.len()..]));
        }
        offset += line.len();
    }
    None
}

/// Parsed frontmatter; None when the file has none.
pub(crate) fn parse(text: &str) -> Option<Map<String, Value>> {
    split(text).map(|(yaml, _)| parse_yaml(yaml))
}

pub(crate) fn get_str(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key)? {
        Value::String(s) => Some(s.clone()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

pub(crate) fn get_bool(map: &Map<String, Value>, key: &str) -> bool {
    match map.get(key) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s == "true",
        _ => false,
    }
}

/// A list key; a comma-separated string counts as a list (`tools: Read, Grep`).
pub(crate) fn get_list(map: &Map<String, Value>, key: &str) -> Option<Vec<String>> {
    match map.get(key)? {
        Value::Array(items) => Some(
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
        ),
        Value::String(s) => Some(
            s.split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
        ),
        _ => None,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn is_blank(line: &str) -> bool {
    line.trim().is_empty()
}

pub(crate) fn parse_yaml(yaml: &str) -> Map<String, Value> {
    let lines: Vec<&str> = yaml.lines().collect();
    let mut map = Map::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if is_blank(line) || line.trim_start().starts_with('#') || indent_of(line) > 0 {
            continue;
        }
        let Some((key, rest)) = split_key(line) else {
            continue;
        };
        // Lines that belong to this key: indented ones, blank ones, and an indentless `- ` list.
        let start = i;
        while i < lines.len()
            && (is_blank(lines[i])
                || indent_of(lines[i]) > 0
                || (rest.is_empty() && lines[i].starts_with("- ")))
        {
            i += 1;
        }
        let more = &lines[start..i];
        map.insert(key, value_of(rest, more));
    }
    map
}

fn split_key(line: &str) -> Option<(String, &str)> {
    let bytes = line.as_bytes();
    let mut idx = None;
    for (pos, &b) in bytes.iter().enumerate() {
        if b == b':'
            && (pos + 1 == bytes.len() || bytes[pos + 1] == b' ' || bytes[pos + 1] == b'\t')
        {
            idx = Some(pos);
            break;
        }
    }
    let idx = idx?;
    let key = line[..idx].trim().trim_matches(['"', '\'']).to_string();
    if key.is_empty() {
        return None;
    }
    Some((key, line[idx + 1..].trim()))
}

fn value_of(rest: &str, more: &[&str]) -> Value {
    if rest.starts_with('|') || rest.starts_with('>') {
        return Value::String(block_scalar(rest, more));
    }
    if rest.starts_with('"') || rest.starts_with('\'') {
        return Value::String(quoted(rest, more));
    }
    if rest.starts_with('[') {
        let mut text = rest.to_string();
        for line in more {
            text.push(' ');
            text.push_str(line.trim());
        }
        return flow_seq(&text);
    }
    // ` #` inside a plain value is kept: Claude Code reads `… PR #1234 …` whole (zworkflow
    // `silent-failure-hunter`, whose roster line in the 2026-10-13 capture carries the full text).
    if rest.is_empty() || rest.starts_with('#') {
        let items: Vec<&str> = more.iter().filter(|l| !is_blank(l)).copied().collect();
        if items
            .first()
            .is_some_and(|l| l.trim_start().starts_with("- "))
        {
            return Value::Array(
                items
                    .iter()
                    .filter_map(|l| l.trim_start().strip_prefix("- "))
                    .map(|item| Value::String(unquote(item.trim())))
                    .collect(),
            );
        }
        return Value::Null;
    }
    // Plain scalar, possibly continued on more-indented lines (folded with single spaces).
    let mut text = rest.to_string();
    for line in more {
        if is_blank(line) {
            text.push('\n');
        } else {
            if !text.ends_with('\n') {
                text.push(' ');
            }
            text.push_str(line.trim());
        }
    }
    let text = text.trim_end().to_string();
    match text.as_str() {
        "true" | "True" | "TRUE" => Value::Bool(true),
        "false" | "False" | "FALSE" => Value::Bool(false),
        "null" | "~" => Value::Null,
        _ => Value::String(text),
    }
}

fn strip_comment(s: &str) -> &str {
    match s.find(" #") {
        Some(pos) => s[..pos].trim_end(),
        None if s.starts_with('#') => "",
        None => s,
    }
}

fn unquote(s: &str) -> String {
    if (s.starts_with('"') && s.ends_with('"') && s.len() >= 2)
        || (s.starts_with('\'') && s.ends_with('\'') && s.len() >= 2)
    {
        quoted(s, &[])
    } else {
        strip_comment(s).to_string()
    }
}

fn flow_seq(text: &str) -> Value {
    let inner = text
        .trim()
        .trim_start_matches('[')
        .split(']')
        .next()
        .unwrap_or("");
    Value::Array(
        inner
            .split(',')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(|p| Value::String(unquote(p)))
            .collect(),
    )
}

/// A quoted scalar that may span lines (line breaks fold to spaces, blank lines to `\n`).
fn quoted(first: &str, more: &[&str]) -> String {
    let quote = first.chars().next().unwrap_or('"');
    let mut raw = first[1..].to_string();
    for line in more {
        raw.push('\n');
        raw.push_str(line);
    }
    // Find the closing quote.
    let chars: Vec<char> = raw.chars().collect();
    let mut body = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if quote == '\'' {
            if c == '\'' {
                if chars.get(i + 1) == Some(&'\'') {
                    body.push('\'');
                    i += 2;
                    continue;
                }
                break;
            }
            body.push(c);
            i += 1;
        } else {
            if c == '\\' {
                if let Some(&n) = chars.get(i + 1) {
                    body.push('\\');
                    body.push(n);
                    i += 2;
                    continue;
                }
            }
            if c == '"' {
                break;
            }
            body.push(c);
            i += 1;
        }
    }
    let folded = fold_lines(&body);
    if quote == '"' {
        unescape(&folded)
    } else {
        folded
    }
}

fn fold_lines(text: &str) -> String {
    if !text.contains('\n') {
        return text.to_string();
    }
    let mut out = String::new();
    let mut pending_blank = 0;
    for (n, line) in text.split('\n').enumerate() {
        let piece = if n == 0 { line.trim_end() } else { line.trim() };
        if n > 0 && piece.is_empty() {
            pending_blank += 1;
            continue;
        }
        if n > 0 {
            if pending_blank > 0 {
                for _ in 0..pending_blank {
                    out.push('\n');
                }
            } else {
                out.push(' ');
            }
        }
        pending_blank = 0;
        out.push_str(piece);
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('"') => out.push('"'),
            Some('\\') => out.push('\\'),
            Some('/') => out.push('/'),
            Some(' ') => out.push(' '),
            Some('u') => {
                let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    out.push(ch);
                }
            }
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `|`, `|-`, `|+`, `>`, `>-`, `>+` (an indentation digit is accepted and used).
fn block_scalar(header: &str, more: &[&str]) -> String {
    let header = strip_comment(header);
    let folded = header.starts_with('>');
    let chomp = if header.contains('-') {
        '-'
    } else if header.contains('+') {
        '+'
    } else {
        ' '
    };
    let explicit: Option<usize> = header
        .chars()
        .find(|c| c.is_ascii_digit())
        .and_then(|c| c.to_digit(10))
        .map(|d| d as usize);
    let indent = explicit.unwrap_or_else(|| {
        more.iter()
            .find(|l| !is_blank(l))
            .map(|l| indent_of(l))
            .unwrap_or(0)
    });
    let lines: Vec<String> = more
        .iter()
        .map(|l| {
            if is_blank(l) {
                String::new()
            } else if l.len() >= indent {
                l[indent..].to_string()
            } else {
                l.trim_start().to_string()
            }
        })
        .collect();
    // Trailing blank lines are governed by chomping.
    let content_end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map(|p| p + 1)
        .unwrap_or(0);
    let trailing_blanks = lines.len() - content_end;
    let content = &lines[..content_end];
    let mut out = String::new();
    if folded {
        let mut first = true;
        let mut empties = 0;
        let mut prev_more = false;
        for line in content {
            if line.is_empty() {
                empties += 1;
                continue;
            }
            let more_indented = line.starts_with(' ') || line.starts_with('\t');
            if first {
                for _ in 0..empties {
                    out.push('\n');
                }
            } else if empties == 0 {
                out.push(if more_indented || prev_more {
                    '\n'
                } else {
                    ' '
                });
            } else {
                if more_indented || prev_more {
                    out.push('\n');
                }
                for _ in 0..empties {
                    out.push('\n');
                }
            }
            out.push_str(line);
            first = false;
            empties = 0;
            prev_more = more_indented;
        }
    } else {
        out = content.join("\n");
    }
    if content_end > 0 {
        match chomp {
            '-' => {}
            '+' => {
                out.push('\n');
                for _ in 0..trailing_blanks {
                    out.push('\n');
                }
            }
            _ => out.push('\n'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fm(text: &str) -> Map<String, Value> {
        parse(text).expect("frontmatter")
    }

    #[test]
    fn plain_quoted_and_bool() {
        let m = fm("---\nname: zhuge\ndescription: \"전략가 \\\"quoted\\\"\"\nx: 'it''s'\ndisable-model-invocation: true\n---\nbody\n");
        assert_eq!(m["name"], json!("zhuge"));
        assert_eq!(m["description"], json!("전략가 \"quoted\""));
        assert_eq!(m["x"], json!("it's"));
        assert_eq!(m["disable-model-invocation"], json!(true));
    }

    #[test]
    fn folded_and_literal_blocks() {
        let m = fm("---\nname: s\ndescription: >\n  Use when the user\n  hands one item.\nwhen_to_use: >-\n  Triggers: a,\n  b\nlit: |\n  line 1\n  line 2\n---\n");
        assert_eq!(
            m["description"],
            json!("Use when the user hands one item.\n")
        );
        assert_eq!(m["when_to_use"], json!("Triggers: a, b"));
        assert_eq!(m["lit"], json!("line 1\nline 2\n"));
    }

    #[test]
    fn folded_keeps_paragraph_breaks() {
        let m = fm("---\nd: >\n  a\n  b\n\n  c\n---\n");
        assert_eq!(m["d"], json!("a b\nc\n"));
    }

    #[test]
    fn lists_flow_and_block() {
        let m = fm("---\npaths:\n  - \"src/**/*.rs\"\n  - docs/*.md\ntools: [Read, \"Grep\"]\nother:\n- a\n- b\n---\n");
        assert_eq!(m["paths"], json!(["src/**/*.rs", "docs/*.md"]));
        assert_eq!(m["tools"], json!(["Read", "Grep"]));
        assert_eq!(m["other"], json!(["a", "b"]));
        assert_eq!(
            get_list(&fm("---\ntools: Read, Grep\n---\n"), "tools").unwrap(),
            vec!["Read", "Grep"]
        );
    }

    #[test]
    fn plain_scalar_continues_on_indented_lines_and_keeps_hashes() {
        let m = fm("---\ndescription: Use when foo\n  and PR #1234: x\nname: x\n---\n");
        assert_eq!(m["description"], json!("Use when foo and PR #1234: x"));
        assert_eq!(m["name"], json!("x"));
    }

    #[test]
    fn nested_maps_are_null_and_no_frontmatter_is_none() {
        let m = fm("---\nhooks:\n  PreToolUse:\n    - matcher: x\nname: y\n---\n");
        assert_eq!(m["hooks"], Value::Null);
        assert_eq!(m["name"], json!("y"));
        assert!(parse("<!-- persona -->\nbody\n").is_none());
        assert!(parse("---\nname: never closed\n").is_none());
    }
}
