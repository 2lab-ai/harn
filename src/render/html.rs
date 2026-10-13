//! `harn report --html`: one self-contained, read-only page for the human (A13). Inline CSS
//! only, no script, no external resource, no form control; rows become cards under 600 px.

use super::common::{
    cost_column, default_action, denominator, facts, freed_cell, kb, origin_short, partition,
    percent, request_bytes, share_tenths, sorted, Glyphs, UNICODE,
};
use crate::model::{Document, Item, Tree};

const CSS: &str = r#"
:root{--fg:#1d1f23;--muted:#5d6470;--line:#d9dce1;--track:#eceef1;--bar:#3b6fd8;--warn:#9a5b00;--bg:#fff}
@media (prefers-color-scheme:dark){:root{--fg:#e3e5e8;--muted:#9aa1ab;--line:#3a3f47;--track:#2a2e35;--bar:#6e9bf0;--warn:#e0a84a;--bg:#16181c}}
*{box-sizing:border-box}
html{-webkit-text-size-adjust:100%}
body{margin:0 auto;max-width:80rem;padding:1rem;font:14px/1.45 system-ui,-apple-system,"Segoe UI",sans-serif;color:var(--fg);background:var(--bg);overflow-wrap:break-word}
h1{font-size:1.3rem;margin:0 0 .25rem}
h2{font-size:1.1rem;margin:1.5rem 0 .25rem}
h3{font-size:.95rem;margin:1rem 0 .25rem;color:var(--muted)}
code,.id{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;font-size:.85em;overflow-wrap:anywhere;word-break:break-all}
.meta,.id,footer{color:var(--muted)}
ul.facts{list-style:none;padding:0;margin:.5rem 0}
ul.facts li{margin:.15rem 0}
.warn,.signal{color:var(--warn)}
table.rows{border-collapse:collapse;width:100%;table-layout:auto}
table.rows th,table.rows td{text-align:left;vertical-align:top;padding:.3rem .5rem;border-bottom:1px solid var(--line)}
table.rows th{font-weight:600;color:var(--muted);white-space:nowrap}
table.rows td.num{text-align:right;white-space:nowrap}
table.rows td.share,table.rows td.mode{white-space:nowrap}
table.rows td.item{padding-left:calc(.5rem + var(--d,0) * 1.2rem)}
td.item .id{display:block}
details.why{color:var(--muted);font-size:.85em}
details.why summary{cursor:pointer}
details.why ol{margin:.2rem 0;padding-left:1.2rem}
details.why li{font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;overflow-wrap:anywhere;word-break:break-all}
.bar{display:inline-block;width:5rem;height:.6rem;background:var(--track);vertical-align:middle;margin-right:.35rem}
.bar i{display:block;height:100%;background:var(--bar)}
ol.notes{color:var(--muted);padding-left:1.5rem}
footer{margin-top:2rem;font-size:.85em}
@media (max-width:600px){
table.rows,table.rows tbody,table.rows tr,table.rows td{display:block;width:auto}
table.rows thead{display:none}
table.rows tr{border:1px solid var(--line);border-radius:6px;margin:.5rem 0 .5rem calc(var(--d,0) * .8rem);padding:.35rem .6rem}
table.rows td{border:0;padding:.1rem 0;text-align:left;white-space:normal}
table.rows td.item{padding-left:0}
table.rows td.num{text-align:left}
table.rows td::before{content:attr(data-l);display:inline-block;min-width:6.5rem;color:var(--muted)}
table.rows td.item::before{display:none}
table.rows td:empty{display:none}
}
"#;

/// The page: header, one section per agent (facts, the main rows, the runtime block, the signal
/// notes), a footer with `doc.generated_at` and the `harn --version` line.
pub fn html(doc: &Document) -> String {
    let g = &UNICODE;
    let mut dirs: Vec<String> = Vec::new();
    for tree in &doc.agents {
        let dir = tree.target_dir.display().to_string();
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    let dirs = dirs.join(", ");
    let mut out = vec![
        "<!doctype html>".to_string(),
        "<html lang=\"en\">".to_string(),
        "<head>".to_string(),
        "<meta charset=\"utf-8\">".to_string(),
        "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">".to_string(),
        format!("<title>harn report {} {}</title>", g.dash, esc(&dirs)),
        format!("<style>{}</style>", CSS.trim()),
        "</head>".to_string(),
        "<body>".to_string(),
        format!(
            "<header><h1>harn report</h1><p class=\"meta\"><code>{}</code> {} read-only view of the current loadout; change it with <code>harn off</code> / <code>harn apply</code></p></header>",
            esc(&dirs),
            g.dot
        ),
        "<main>".to_string(),
    ];
    for tree in &doc.agents {
        section(&mut out, tree, g);
    }
    if doc.agents.is_empty() {
        out.push("<p>No agent trees.</p>".to_string());
    }
    out.push("</main>".to_string());
    out.push(format!(
        "<footer>generated {} by {}</footer>",
        esc(&doc.generated_at),
        esc(&crate::version_line())
    ));
    out.push("</body>".to_string());
    out.push("</html>".to_string());
    out.join("\n")
}

fn section(out: &mut Vec<String>, tree: &Tree, g: &Glyphs) {
    let f = facts(tree, g, kb);
    out.push(format!("<section class=\"agent\" id=\"{}\">", tree.agent));
    out.push(format!(
        "<h2>{}</h2><p class=\"meta\">target <code>{}</code></p>",
        esc(&f.title),
        esc(&tree.target_dir.display().to_string())
    ));
    out.push("<ul class=\"facts\">".to_string());
    if let Some(drift) = &f.drift {
        out.push(format!("<li class=\"warn\">{}</li>", esc(drift)));
    }
    out.push(format!("<li>{}</li>", esc(&f.totals)));
    for line in &f.tool_search {
        out.push(format!("<li>{}</li>", esc(line)));
    }
    for line in &f.signals {
        out.push(format!("<li class=\"signal\">{}</li>", esc(line)));
    }
    for line in &f.warnings {
        out.push(format!("<li class=\"warn\">{}</li>", esc(line)));
    }
    out.push("</ul>".to_string());

    let ctx = Ctx {
        g,
        denominator: denominator(tree),
        cost_header: if tree.observed_body.is_some() {
            "observed"
        } else {
            "predicted"
        },
    };
    let mut notes: Vec<String> = Vec::new();
    let (main, trailing) = partition(tree);
    table(out, &main, &mut notes, &ctx);
    if !trailing.is_empty() {
        out.push("<h3>runtime / no first-request cost</h3>".to_string());
        table(out, &trailing, &mut notes, &ctx);
    }
    if !notes.is_empty() {
        out.push("<ol class=\"notes\">".to_string());
        out.extend(notes.iter().map(|n| format!("<li>{}</li>", esc(n))));
        out.push("</ol>".to_string());
    }
    out.push("</section>".to_string());
}

struct Ctx<'a> {
    g: &'a Glyphs,
    denominator: u64,
    cost_header: &'static str,
}

const COLUMNS: [&str; 7] = [
    "freed (pred)",
    "",
    "share",
    "item",
    "origin",
    "mode",
    "default action",
];

fn table(out: &mut Vec<String>, items: &[&Item], notes: &mut Vec<String>, ctx: &Ctx) {
    let head: Vec<String> = COLUMNS
        .iter()
        .map(|c| {
            let name = if c.is_empty() { ctx.cost_header } else { c };
            format!("<th>{}</th>", esc(name))
        })
        .collect();
    out.push("<table class=\"rows\">".to_string());
    out.push(format!("<thead><tr>{}</tr></thead>", head.concat()));
    out.push("<tbody>".to_string());
    for item in items {
        rows(out, item, 0, notes, ctx);
    }
    out.push("</tbody>".to_string());
    out.push("</table>".to_string());
}

/// The item's row, then every child (all expanded: a file has no width budget).
fn rows(out: &mut Vec<String>, item: &Item, depth: usize, notes: &mut Vec<String>, ctx: &Ctx) {
    let g = ctx.g;
    let tenths = share_tenths(request_bytes(&item.cost), ctx.denominator);
    let share = match tenths {
        Some(t) => format!(
            "<span class=\"bar\"><i style=\"width:{}\"></i></span>{}",
            percent(t),
            percent(t)
        ),
        None => String::new(),
    };
    let mut label = esc(&item.label);
    if !item.children.is_empty() {
        label.push_str(&format!(" {} {}", g.dot, item.children.len()));
    }
    if let Some(signal) = &item.signal {
        notes.push(format!("{}: {signal}", item.label));
        label.push_str(&format!(" <span class=\"signal\">[{}]</span>", notes.len()));
    }
    let cells = [
        ("freed (pred)", "num", esc(&freed_cell(item, g, kb))),
        (
            ctx.cost_header,
            "num",
            esc(&cost_column(&item.cost, ctx.cost_header == "observed", g)),
        ),
        ("share", "share", share),
        (
            "item",
            "item",
            format!(
                "{label}<code class=\"id\">{}</code>{}",
                esc(&item.id),
                why_details(item)
            ),
        ),
        ("origin", "origin", esc(&origin_short(&item.origin))),
        ("mode", "mode", esc(item.mode.as_str())),
        ("default action", "action", esc(&default_action(item, g))),
    ];
    let tds: Vec<String> = cells
        .iter()
        .map(|(label, class, body)| format!("<td class=\"{class}\" data-l=\"{label}\">{body}</td>"))
        .collect();
    out.push(format!("<tr style=\"--d:{depth}\">{}</tr>", tds.concat()));
    for child in sorted(&item.children) {
        rows(out, child, depth + 1, notes, ctx);
    }
}

/// The admission chain, collapsed (native `<details>`, no script); empty without one.
fn why_details(item: &Item) -> String {
    if item.origin.why.is_empty() {
        return String::new();
    }
    let steps: Vec<String> = item
        .origin
        .why
        .iter()
        .map(|step| format!("<li>{}</li>", esc(step)))
        .collect();
    format!(
        "<details class=\"why\"><summary>why loaded</summary><ol>{}</ol></details>",
        steps.concat()
    )
}

/// HTML text/attribute escape.
fn esc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}
