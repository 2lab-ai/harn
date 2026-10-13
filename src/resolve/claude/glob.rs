//! Path globs for `claudeMdExcludes` and rule `paths:`: `**` (any run of segments), `*` and `?`
//! (within a segment), `[…]` classes, `{a,b}` alternatives, `\` escapes.

pub(crate) fn matches(pattern: &str, path: &str) -> bool {
    let path: Vec<char> = path.chars().collect();
    expand_braces(pattern).iter().any(|p| {
        let p: Vec<char> = p.chars().collect();
        match_at(&p, &path)
    })
}

/// Escapes glob metacharacters so a literal path matches only itself.
pub(crate) fn escape(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if matches!(c, '*' | '?' | '[' | ']' | '{' | '}' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

fn expand_braces(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut depth = 0;
    let mut open = None;
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 1,
            '{' => {
                if depth == 0 {
                    open = Some(i);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let start = open.unwrap_or(0);
                    let prefix: String = chars[..start].iter().collect();
                    let suffix: String = chars[i + 1..].iter().collect();
                    let inner: String = chars[start + 1..i].iter().collect();
                    let mut out = Vec::new();
                    for alt in split_top_level(&inner) {
                        out.extend(expand_braces(&format!("{prefix}{alt}{suffix}")));
                    }
                    return out;
                }
            }
            _ => {}
        }
        i += 1;
    }
    vec![pattern.to_string()]
}

fn split_top_level(inner: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut cur = String::new();
    for c in inner.chars() {
        match c {
            '{' => {
                depth += 1;
                cur.push(c);
            }
            '}' => {
                depth -= 1;
                cur.push(c);
            }
            ',' if depth == 0 => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
}

fn match_at(p: &[char], s: &[char]) -> bool {
    let Some(&first) = p.first() else {
        return s.is_empty();
    };
    match first {
        '*' if p.get(1) == Some(&'*') => {
            let rest = &p[2..];
            // `**/` also matches zero segments.
            if rest.first() == Some(&'/') && match_at(&rest[1..], s) {
                return true;
            }
            (0..=s.len()).any(|k| match_at(rest, &s[k..]))
        }
        '*' => {
            let rest = &p[1..];
            let mut k = 0;
            loop {
                if match_at(rest, &s[k..]) {
                    return true;
                }
                if k == s.len() || s[k] == '/' {
                    return false;
                }
                k += 1;
            }
        }
        '?' => !s.is_empty() && s[0] != '/' && match_at(&p[1..], &s[1..]),
        '[' => match class(p) {
            Some((set, negated, len)) => {
                !s.is_empty()
                    && s[0] != '/'
                    && (set_contains(&set, s[0]) != negated)
                    && match_at(&p[len..], &s[1..])
            }
            None => !s.is_empty() && s[0] == '[' && match_at(&p[1..], &s[1..]),
        },
        '\\' if p.len() > 1 => !s.is_empty() && s[0] == p[1] && match_at(&p[2..], &s[1..]),
        c => !s.is_empty() && s[0] == c && match_at(&p[1..], &s[1..]),
    }
}

/// `[…]` at the start of `p`: (members as ranges, negated, pattern length consumed).
#[allow(clippy::type_complexity)]
fn class(p: &[char]) -> Option<(Vec<(char, char)>, bool, usize)> {
    let mut i = 1;
    let negated = matches!(p.get(1), Some('!') | Some('^'));
    if negated {
        i += 1;
    }
    let mut set = Vec::new();
    let mut first = true;
    while i < p.len() {
        if p[i] == ']' && !first {
            return Some((set, negated, i + 1));
        }
        first = false;
        let lo = p[i];
        if p.get(i + 1) == Some(&'-') && p.get(i + 2).is_some_and(|c| *c != ']') {
            set.push((lo, p[i + 2]));
            i += 3;
        } else {
            set.push((lo, lo));
            i += 1;
        }
    }
    None
}

fn set_contains(set: &[(char, char)], c: char) -> bool {
    set.iter().any(|(lo, hi)| *lo <= c && c <= *hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_stays_in_a_segment_double_star_crosses() {
        assert!(matches("/a/*/CLAUDE.md", "/a/b/CLAUDE.md"));
        assert!(!matches("/a/*/CLAUDE.md", "/a/b/c/CLAUDE.md"));
        assert!(matches("**/CLAUDE.md", "/a/b/c/CLAUDE.md"));
        assert!(matches("/a/**/CLAUDE.md", "/a/CLAUDE.md"));
        assert!(matches("/a/.claude/rules/**", "/a/.claude/rules/x/y.md"));
        assert!(!matches("**/other/CLAUDE.md", "/a/b/CLAUDE.md"));
    }

    #[test]
    fn classes_braces_and_escapes() {
        assert!(matches("src/*.{rs,toml}", "src/main.rs"));
        assert!(matches("src/*.{rs,toml}", "src/Cargo.toml"));
        assert!(!matches("src/*.{rs,toml}", "src/a.md"));
        assert!(matches("file[0-9].md", "file7.md"));
        assert!(!matches("file[!0-9].md", "file7.md"));
        assert!(matches("?.md", "a.md"));
        let literal = "/x/[weird]*/CLAUDE.md";
        assert!(matches(&escape(literal), literal));
        assert!(!matches(&escape(literal), "/x/w*/CLAUDE.md"));
    }
}
